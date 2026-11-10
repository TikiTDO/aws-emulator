// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS certificate-authority slice:
//! `CreateCertificateAuthority` and `DescribeCertificateAuthority`, plus the CA
//! Amazon EKS provisions with a cluster, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The wire harness (`emulator`, `call`, `encode_label`, `Answer`) is this test
//! target's shared one in `tests/all/wire.rs`; this file reuses it rather than
//! keeping a second copy, and its own certificate-authority helpers are shared
//! with the activation suite (`tests/all/certificate_authority_activation.rs`)
//! for the same reason.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (whether an
//! idempotency token is supplied and how two calls' tokens relate, the cluster the
//! CA hangs off, the region the request is signed for, how many CAs a cluster
//! already holds, the cluster's status), and each assertion names its case and
//! prints the actual value.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    derive_certificate_authority_id, CA_DISTRIBUTION_DELAY, CA_FINAL_AUTO_ACTIVATION_LEAD,
    CA_FIRST_AUTO_ACTIVATION_LEAD, CA_VALIDITY, CLUSTER_ACTIVATION_DELAY,
    MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, encode_label, Answer};

/// The region every request in this suite is signed for, except where a case
/// deliberately uses another one.
pub(crate) const REGION: &str = "us-west-2";

/// The smallest `CreateCluster` request this service accepts, for a cluster the
/// certificate-authority calls then act on.
fn minimal_cluster(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-11111111", "subnet-22222222"] },
    })
}

pub(crate) async fn create_cluster(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> Answer {
    call(
        router,
        region,
        "POST",
        "/clusters",
        &minimal_cluster(name).to_string(),
    )
    .await
}

/// `DescribeCluster`: the cluster record the certificate authorities hang off.
pub(crate) async fn describe_cluster(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/clusters/{}", encode_label(cluster)),
        "",
    )
    .await
}

/// `DescribeUpdate`: `GET /clusters/{name}/updates/{updateId}`, which is where the
/// update the append returns is read back from.
pub(crate) async fn describe_update(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    update_id: &str,
) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!(
            "/clusters/{}/updates/{}",
            encode_label(cluster),
            encode_label(update_id)
        ),
        "",
    )
    .await
}

/// `CreateCertificateAuthority`: `POST /clusters/{clusterName}/certificate-authorities`.
pub(crate) async fn append_authority(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    client_request_token: Option<&str>,
) -> Answer {
    let body = match client_request_token {
        Some(token) => serde_json::json!({ "clientRequestToken": token }),
        None => serde_json::json!({}),
    };
    call(
        router,
        region,
        "POST",
        &format!("/clusters/{}/certificate-authorities", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

/// `DescribeCertificateAuthority`:
/// `GET /clusters/{clusterName}/certificate-authorities/{certificateAuthorityId}`.
pub(crate) async fn describe_authority(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    authority_id: &str,
) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!(
            "/clusters/{}/certificate-authorities/{}",
            encode_label(cluster),
            encode_label(authority_id)
        ),
        "",
    )
    .await
}

/// Wait out a clock-owed move, with enough slack that a loaded machine does not
/// race the due instant.
pub(crate) async fn settle(span: Duration) {
    tokio::time::sleep(span + Duration::from_millis(250)).await;
}

/// The id form this emulator mints for a certificate authority and for an update:
/// the RFC 4122 version-4 UUID string shape, which is what the model documents for
/// an update id ("A UUID that is used to track the update", `Update$id`) — the
/// `8-4-4-4-12` hex layout with the version nibble 4 and the variant's two top bits
/// `10`, which is what the service's own formatter sets.
pub(crate) fn is_uuid(value: &str) -> bool {
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == widths.len()
        && groups.iter().zip(widths).all(|(group, width)| {
            group.len() == width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && groups[2].starts_with('4')
        && groups[3]
            .chars()
            .next()
            .is_some_and(|nibble| matches!(nibble, '8' | '9' | 'a' | 'b' | 'A' | 'B'))
}

/// A timestamp member, as restJson1 puts an `@timestamp` on the wire: epoch
/// seconds.
pub(crate) fn epoch(case: &str, member: &str, value: &serde_json::Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected epoch seconds for {member}, got {value:?}"))
}

/// Two instants are the same to well inside a second, which is as close as two
/// epoch-seconds numbers computed from one stored instant need to be.
pub(crate) fn same_instant(left: f64, right: f64) -> bool {
    (left - right).abs() < 0.001
}

pub(crate) fn str_member(case: &str, member: &str, value: &serde_json::Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {value:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// RULE: appending a certificate authority records one CA on the named cluster —
// created by the CUSTOMER, not yet signing, its trust distribution in progress —
// and answers with that CA's summary plus the asynchronous update that tracks the
// distribution; a later read returns exactly the recorded values, with the
// validity period and the auto-activation events computed from it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_appended_certificate_authority_reads_back_exactly_as_stored() {
    let router = emulator();

    // Rows differ along whether an idempotency token is supplied and along the
    // cluster the CA hangs off (so the ids must differ between them).
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("no idempotency token", "ca-plain", None),
        ("an idempotency token", "ca-token", Some("token-ca-read-back")),
        ("another cluster name shape", "Ca9_mixed-Name", None),
        ("a third cluster, to pin id uniqueness", "ca-third", None),
    ];

    for (case, cluster, _) in &cases {
        let created = create_cluster(&router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let mut minted_ids = BTreeSet::new();
    for (case, cluster, token) in &cases {
        let cluster_record = describe_cluster(&router, REGION, cluster).await;
        let cluster_authority_data = str_member(
            case,
            "cluster.certificateAuthority.data",
            &cluster_record.body["cluster"]["certificateAuthority"]["data"],
        );

        let appended = append_authority(&router, REGION, cluster, *token).await;
        assert_eq!(
            appended.status, 200,
            "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
        );

        let summary = &appended.body["certificateAuthority"];
        let id = str_member(case, "certificateAuthority.id", &summary["id"]);
        assert!(
            is_uuid(&id),
            "case {case:?}: expected a UUID-shaped certificate authority id, got {id:?}"
        );
        assert!(
            minted_ids.insert(id.clone()),
            "case {case:?}: every appended certificate authority gets its own id, \
             but {id:?} was minted twice"
        );
        assert_eq!(
            summary["createdBy"],
            serde_json::json!("CUSTOMER"),
            "case {case:?}: a CA the caller appends is created by CUSTOMER, got {:?}",
            summary["createdBy"]
        );
        assert_eq!(
            summary["signingStatus"],
            serde_json::json!("NOT_USED"),
            "case {case:?}: appending a successor does not begin signing, got {:?}",
            summary["signingStatus"]
        );
        assert_eq!(
            summary["distributionStatus"],
            serde_json::json!("IN_PROGRESS"),
            "case {case:?}: the successor's trust distribution starts in progress, got {:?}",
            summary["distributionStatus"]
        );
        assert!(
            summary["createdAt"].is_number(),
            "case {case:?}: expected an epoch-seconds createdAt, got {:?}",
            summary["createdAt"]
        );
        assert!(
            summary["activatedAt"].is_null() && summary["activatedBy"].is_null(),
            "case {case:?}: a CA that has never been activated reports neither \
             activatedAt nor activatedBy, got {summary:?}"
        );

        // The asynchronous update the operation returns: it tracks the trust
        // distribution, so it is in progress and names this CA.
        let update = &appended.body["update"];
        let update_id = str_member(case, "update.id", &update["id"]);
        assert!(
            is_uuid(&update_id),
            "case {case:?}: expected a UUID-shaped update id, got {update_id:?}"
        );
        assert_eq!(
            update["type"],
            serde_json::json!("CertificateAuthorityUpdate"),
            "case {case:?}: expected the CertificateAuthorityUpdate type, got {:?}",
            update["type"]
        );
        assert_eq!(
            update["status"],
            serde_json::json!("InProgress"),
            "case {case:?}: the update is in progress while the distribution is, got {:?}",
            update["status"]
        );
        assert_eq!(
            update["params"],
            serde_json::json!([{ "type": "CertificateAuthorityId", "value": id }]),
            "case {case:?}: the update names the certificate authority it is about, got {:?}",
            update["params"]
        );
        assert!(
            update["createdAt"].is_number(),
            "case {case:?}: expected an epoch-seconds update createdAt, got {:?}",
            update["createdAt"]
        );

        // The update is a resource of the cluster, not just a value in this
        // response: `DescribeUpdate` reads back the same object.
        let read_update = describe_update(&router, REGION, cluster, &update_id).await;
        assert_eq!(
            read_update.status, 200,
            "case {case:?}: the append's update is readable, got {read_update:?}"
        );
        assert_eq!(
            read_update.body["update"], *update,
            "case {case:?}: DescribeUpdate must answer with the update the append \
             returned, got {:?}",
            read_update.body["update"]
        );

        // The round trip: the recorded CA, read back through
        // `DescribeCertificateAuthority`.
        let described = describe_authority(&router, REGION, cluster, &id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeCertificateAuthority, got {described:?}"
        );
        let authority = &described.body["certificateAuthority"];
        for member in [
            "id",
            "createdAt",
            "createdBy",
            "signingStatus",
            "distributionStatus",
        ] {
            assert_eq!(
                authority[member], summary[member],
                "case {case:?}: the read-back {member} must be the value the append \
                 recorded, got {:?}",
                authority[member]
            );
        }

        // `data` is "the Base64-encoded public certificate of the certificate
        // authority": it decodes, re-encodes to the same blob, and belongs to this
        // CA rather than being the cluster signer's certificate.
        let data = str_member(case, "certificateAuthority.data", &authority["data"]);
        let decoded = aws_smithy_types::base64::decode(&data).unwrap_or_else(|error| {
            panic!("case {case:?}: expected base64 in data, got {data:?} ({error})")
        });
        assert_eq!(
            aws_smithy_types::base64::encode(&decoded),
            data,
            "case {case:?}: the base64 blob must round trip unchanged"
        );
        assert!(
            String::from_utf8(decoded.clone())
                .map(|pem| pem.contains("CERTIFICATE"))
                .unwrap_or(false),
            "case {case:?}: the decoded blob is a PEM certificate body, got {:?} bytes",
            decoded.len()
        );
        assert_ne!(
            data, cluster_authority_data,
            "case {case:?}: an appended CA carries its own certificate, not the \
             cluster signer's"
        );

        // The validity period is recorded, and the auto-activation events are
        // computed from it.
        let not_before = epoch(case, "validity.notBefore", &authority["validity"]["notBefore"]);
        let not_after = epoch(case, "validity.notAfter", &authority["validity"]["notAfter"]);
        let created_at = epoch(case, "createdAt", &authority["createdAt"]);
        assert!(
            same_instant(not_before, created_at),
            "case {case:?}: validity starts when the CA was created, got \
             notBefore {not_before} and createdAt {created_at}"
        );
        assert!(
            same_instant(not_after - not_before, CA_VALIDITY.as_secs_f64()),
            "case {case:?}: expected a {} second validity period, got {}",
            CA_VALIDITY.as_secs(),
            not_after - not_before
        );
        let first = epoch(
            case,
            "scheduledEvents.firstAutoActivation",
            &authority["scheduledEvents"]["firstAutoActivation"],
        );
        let final_activation = epoch(
            case,
            "scheduledEvents.finalAutoActivation",
            &authority["scheduledEvents"]["finalAutoActivation"],
        );
        assert!(
            same_instant(first, not_after - CA_FIRST_AUTO_ACTIVATION_LEAD.as_secs_f64()),
            "case {case:?}: firstAutoActivation is computed from notAfter, got {first}"
        );
        assert!(
            same_instant(
                final_activation,
                not_after - CA_FINAL_AUTO_ACTIVATION_LEAD.as_secs_f64()
            ),
            "case {case:?}: finalAutoActivation is computed from notAfter, got \
             {final_activation}"
        );
        assert!(
            first < final_activation,
            "case {case:?}: Amazon EKS may activate first and will activate finally, \
             so {first} must come before {final_activation}"
        );
        assert_eq!(
            authority["rollbackAvailable"],
            serde_json::json!(false),
            "case {case:?}: nothing has superseded this CA, so no rollback to it is \
             available, got {:?}",
            authority["rollbackAvailable"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the certificate authority Amazon EKS provisions with a cluster is a CA
// resource of that cluster — created by EKS, the signer, fully distributed — and
// its certificate is what the cluster reports as `certificateAuthority.data`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_certificate_authority_eks_provisions_with_a_cluster_is_the_signer() {
    let router = emulator();

    // Rows differ along the cluster, whose ARN is what its first CA's id is derived
    // from; the id is computed here with the service's own derivation because
    // `ListCertificateAuthorities` is still a 501 with its own gap issue, so there is
    // no other way for a caller to learn it yet.
    let cases = ["signer-one", "signer-two"];

    let mut arns = Vec::new();
    for case in cases {
        let created = create_cluster(&router, REGION, case).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        arns.push(str_member(
            case,
            "cluster.arn",
            &created.body["cluster"]["arn"],
        ));
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let mut signer_ids = BTreeSet::new();
    for (case, arn) in cases.iter().zip(&arns) {
        let cluster_record = describe_cluster(&router, REGION, case).await;
        let cluster_authority_data = str_member(
            case,
            "cluster.certificateAuthority.data",
            &cluster_record.body["cluster"]["certificateAuthority"]["data"],
        );

        let signer_id = derive_certificate_authority_id(arn, 0);
        assert!(
            signer_ids.insert(signer_id.clone()),
            "case {case:?}: each cluster's initial CA has its own id, but \
             {signer_id:?} appeared twice"
        );

        let described = describe_authority(&router, REGION, case, &signer_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the cluster's own CA is readable, got {described:?}"
        );
        let authority = &described.body["certificateAuthority"];
        assert_eq!(
            authority["createdBy"],
            serde_json::json!("EKS"),
            "case {case:?}: a CA Amazon EKS provisions is created by EKS, got {:?}",
            authority["createdBy"]
        );
        assert_eq!(
            authority["signingStatus"],
            serde_json::json!("IN_USE"),
            "case {case:?}: the cluster's own CA signs for it, got {:?}",
            authority["signingStatus"]
        );
        assert_eq!(
            authority["distributionStatus"],
            serde_json::json!("COMPLETE"),
            "case {case:?}: the cluster's own CA is already distributed, got {:?}",
            authority["distributionStatus"]
        );
        assert_eq!(
            authority["activatedBy"],
            serde_json::json!("EKS"),
            "case {case:?}: Amazon EKS activated it, got {:?}",
            authority["activatedBy"]
        );
        assert!(
            authority["activatedAt"].is_number(),
            "case {case:?}: an activated CA reports when, got {:?}",
            authority["activatedAt"]
        );
        assert_eq!(
            authority["data"],
            serde_json::json!(cluster_authority_data),
            "case {case:?}: the cluster reports its signer's certificate, got {:?}",
            authority["data"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a distribution move the clock owes is materialised by whichever read first
// runs after it is due — and not before — and the update that tracks it reports
// the state of the work it tracks.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_appended_authority_distributes_once_the_clock_owes_it() {
    let router = emulator();
    let cases = [("first cluster", "dist-a"), ("second cluster", "dist-b")];

    for (case, cluster) in cases {
        let created = create_cluster(&router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let mut appended_ids = Vec::new();
    for (case, cluster) in cases {
        let token = format!("token-{cluster}");
        let appended = append_authority(&router, REGION, cluster, Some(&token)).await;
        assert_eq!(
            appended.status, 200,
            "case {case:?}: expected 200 from CreateCertificateAuthority, got {appended:?}"
        );
        let id = str_member(
            case,
            "certificateAuthority.id",
            &appended.body["certificateAuthority"]["id"],
        );

        // A caller acting straight after the append still observes the transient
        // state: the span is never zero.
        let immediately = describe_authority(&router, REGION, cluster, &id).await;
        assert_eq!(
            immediately.body["certificateAuthority"]["distributionStatus"],
            serde_json::json!("IN_PROGRESS"),
            "case {case:?}: a read straight after the append still sees IN_PROGRESS, got {:?}",
            immediately.body["certificateAuthority"]["distributionStatus"]
        );
        // The cluster carries the update while it runs.
        let cluster_now = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            cluster_now.body["cluster"]["status"],
            serde_json::json!("UPDATING"),
            "case {case:?}: the cluster is UPDATING while the append's update runs, got {:?}",
            cluster_now.body["cluster"]["status"]
        );
        let update_id = str_member(case, "update.id", &appended.body["update"]["id"]);
        appended_ids.push((case, cluster, id, token, update_id));
    }

    settle(CA_DISTRIBUTION_DELAY).await;

    for (case, cluster, id, token, update_id) in &appended_ids {
        let described = describe_authority(&router, REGION, cluster, id).await;
        assert_eq!(
            described.body["certificateAuthority"]["distributionStatus"],
            serde_json::json!("COMPLETE"),
            "case {case:?}: once the distribution delay has passed the CA is \
             COMPLETE, got {:?}",
            described.body["certificateAuthority"]["distributionStatus"]
        );
        // Re-reading does not move it again or disturb anything else.
        let twice = describe_authority(&router, REGION, cluster, id).await;
        assert_eq!(
            twice.body["certificateAuthority"], described.body["certificateAuthority"],
            "case {case:?}: a second read must answer identically"
        );
        // The CA is trusted but still not the signer: distribution completing is not
        // an activation.
        assert_eq!(
            described.body["certificateAuthority"]["signingStatus"],
            serde_json::json!("NOT_USED"),
            "case {case:?}: a distributed successor still does not sign, got {:?}",
            described.body["certificateAuthority"]["signingStatus"]
        );

        // The update that tracks the distribution now reports it finished — both
        // where it is stored and in the absorbed retry — and the cluster is back to
        // ACTIVE.
        let read_update = describe_update(&router, REGION, cluster, update_id).await;
        assert_eq!(
            read_update.body["update"]["status"],
            serde_json::json!("Successful"),
            "case {case:?}: the stored update is successful once the distribution is \
             complete, got {:?}",
            read_update.body["update"]["status"]
        );
        let cluster_now = describe_cluster(&router, REGION, cluster).await;
        assert_eq!(
            cluster_now.body["cluster"]["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: the cluster is ACTIVE again once the update completed, got {:?}",
            cluster_now.body["cluster"]["status"]
        );

        let retried = append_authority(&router, REGION, cluster, Some(token)).await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: a retry with the same token is absorbed, got {retried:?}"
        );
        assert_eq!(
            retried.body["certificateAuthority"]["id"],
            serde_json::json!(id),
            "case {case:?}: the retry answers with the CA the first call appended, got {:?}",
            retried.body["certificateAuthority"]["id"]
        );
        assert_eq!(
            retried.body["update"]["status"],
            serde_json::json!("Successful"),
            "case {case:?}: the update is successful once the distribution is \
             complete, got {:?}",
            retried.body["update"]["status"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster holds at most the documented number of certificate authorities
// at a time (the signer plus one successor); appending past that ceiling is
// refused as a request that is invalid given the state the cluster is in, and
// writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_holds_at_most_the_documented_number_of_authorities() {
    let router = emulator();
    let cases = [("one cluster", "ceiling-a"), ("another", "ceiling-b")];

    for (case, cluster) in cases {
        let created = create_cluster(&router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    for (case, cluster) in cases {
        // The cluster already holds the CA Amazon EKS provisioned with it, so the
        // ceiling leaves room for one fewer append than the ceiling itself. Each
        // append starts an asynchronous update, and a cluster runs one at a time, so
        // the next call waits for it.
        let mut appended = Vec::new();
        for index in 1..MAX_CERTIFICATE_AUTHORITIES_PER_CLUSTER {
            let answer =
                append_authority(&router, REGION, cluster, Some(&format!("tok-{index}"))).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: append {index} is within the ceiling, got {answer:?}"
            );
            appended.push(str_member(
                case,
                "certificateAuthority.id",
                &answer.body["certificateAuthority"]["id"],
            ));

            // While that update runs the cluster takes no other: one at a time.
            let during = append_authority(&router, REGION, cluster, Some("tok-concurrent")).await;
            assert_eq!(
                during.status, 409,
                "case {case:?}: a second append while update {index} runs expected 409, \
                 got {during:?}"
            );
            assert_eq!(
                during.error_type.as_deref(),
                Some("ResourceInUseException"),
                "case {case:?}: expected ResourceInUseException, got {during:?}"
            );
            settle(CA_DISTRIBUTION_DELAY).await;
        }

        let past = append_authority(&router, REGION, cluster, Some("tok-past-ceiling")).await;
        assert_eq!(
            past.status, 400,
            "case {case:?}: past the ceiling expected 400, got {past:?}"
        );
        assert_eq!(
            past.error_type.as_deref(),
            Some("InvalidRequestException"),
            "case {case:?}: expected InvalidRequestException, got {past:?}"
        );

        // The refused call wrote nothing: every CA appended before it is still
        // readable and unchanged.
        for id in &appended {
            let described = describe_authority(&router, REGION, cluster, id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the CA {id:?} survives a refused append, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: `clientRequestToken` is an `@idempotencyToken` — a second call carrying the
// same non-blank token is the SAME request and is answered with the CA the first
// one appended; any other pair of calls is two requests, and the second is held to
// the ceiling.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retry_with_the_same_idempotency_token_is_absorbed() {
    let router = emulator();

    // Rows differ along the relationship between the two calls' tokens: equal,
    // different, both absent, and both blank (a token that identifies nothing).
    /// One row: the case name, the cluster it runs on, the two calls' tokens, and
    /// whether the second call is a retry of the first.
    struct Row {
        case: &'static str,
        cluster: &'static str,
        first_token: Option<&'static str>,
        second_token: Option<&'static str>,
        absorbed: bool,
    }

    let cases = vec![
        Row {
            case: "the same token twice",
            cluster: "idem-same",
            first_token: Some("tok"),
            second_token: Some("tok"),
            absorbed: true,
        },
        Row {
            case: "two different tokens",
            cluster: "idem-different",
            first_token: Some("tok-a"),
            second_token: Some("tok-b"),
            absorbed: false,
        },
        Row {
            case: "no token on either call",
            cluster: "idem-absent",
            first_token: None,
            second_token: None,
            absorbed: false,
        },
        Row {
            case: "a blank token on both calls",
            cluster: "idem-blank",
            first_token: Some("   "),
            second_token: Some("   "),
            absorbed: false,
        },
    ];

    for row in &cases {
        let (case, cluster) = (row.case, row.cluster);
        let created = create_cluster(&router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let mut first_ids = Vec::new();
    for row in &cases {
        let (case, cluster) = (row.case, row.cluster);
        let first = append_authority(&router, REGION, cluster, row.first_token).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first append succeeds, got {first:?}"
        );
        let first_id = str_member(
            case,
            "certificateAuthority.id",
            &first.body["certificateAuthority"]["id"],
        );

        // A retry that arrives while the append's update is still running is
        // absorbed too: idempotency does not wait for the work to finish.
        if row.absorbed {
            let during = append_authority(&router, REGION, cluster, row.second_token).await;
            assert_eq!(
                during.body["certificateAuthority"]["id"],
                serde_json::json!(first_id),
                "case {case:?}: an in-flight retry returns the original CA, got {during:?}"
            );
        }
        first_ids.push((row, first, first_id));
    }

    // The appends' updates complete, so a call that is NOT a retry is judged on the
    // ceiling rather than on the update still running.
    settle(CA_DISTRIBUTION_DELAY).await;

    for (row, first, first_id) in &first_ids {
        let (case, cluster) = (row.case, row.cluster);
        let second = append_authority(&router, REGION, cluster, row.second_token).await;
        if row.absorbed {
            assert_eq!(
                second.status, 200,
                "case {case:?}: an absorbed retry succeeds, got {second:?}"
            );
            assert_eq!(
                second.body["certificateAuthority"]["id"],
                serde_json::json!(first_id),
                "case {case:?}: an absorbed retry returns the original CA, got {:?}",
                second.body["certificateAuthority"]["id"]
            );
            assert_eq!(
                second.body["certificateAuthority"]["createdAt"],
                first.body["certificateAuthority"]["createdAt"],
                "case {case:?}: an absorbed retry returns the original createdAt"
            );
            assert_eq!(
                second.body["update"]["id"],
                first.body["update"]["id"],
                "case {case:?}: an absorbed retry reports the original update, got {:?}",
                second.body["update"]["id"]
            );
        } else {
            // Not a retry, so it is a second append — which the cluster, already
            // holding the signer and this successor, has no room for.
            assert_eq!(
                second.error_type.as_deref(),
                Some("InvalidRequestException"),
                "case {case:?}: a call that is not a retry is held to the ceiling, \
                 got {second:?}"
            );
            let described = describe_authority(&router, REGION, cluster, first_id).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the first CA is untouched by the refusal, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a cluster name resolves only within the region the request is signed for,
// and a certificate authority id only on its own cluster; anything else is 404
// `ResourceNotFoundException`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_clusters_and_authorities_are_not_found() {
    let router = emulator();
    const OTHER_REGION: &str = "eu-central-1";

    // The same name in two regions is two independent clusters, each with its own
    // certificate authorities.
    for region in [REGION, OTHER_REGION] {
        let created = create_cluster(&router, region, "shared-name").await;
        assert_eq!(
            created.status, 200,
            "region {region:?}: expected 200 from CreateCluster, got {created:?}"
        );
    }
    settle(CLUSTER_ACTIVATION_DELAY).await;

    let appended = append_authority(&router, REGION, "shared-name", Some("tok-region")).await;
    assert_eq!(
        appended.status, 200,
        "expected 200 from CreateCertificateAuthority, got {appended:?}"
    );
    let appended_id = str_member(
        "region isolation",
        "certificateAuthority.id",
        &appended.body["certificateAuthority"]["id"],
    );

    // Rows: an unknown cluster on both operations, an unknown CA id in two shapes,
    // and a CA that exists in another region.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "an unknown cluster",
            REGION,
            "no-such-cluster",
            &appended_id,
        ),
        (
            "a well-formed but unused CA id",
            REGION,
            "shared-name",
            "00000000-0000-4000-8000-000000000000",
        ),
        (
            "an id that is not a UUID at all",
            REGION,
            "shared-name",
            "not-an-id",
        ),
        (
            "a CA of the same-named cluster in another region",
            OTHER_REGION,
            "shared-name",
            &appended_id,
        ),
    ];

    for (case, region, cluster, authority_id) in &cases {
        let described = describe_authority(&router, region, cluster, authority_id).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: expected 404 from DescribeCertificateAuthority, got {described:?}"
        );
        assert_eq!(
            described.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {described:?}"
        );
    }

    let appended_to_unknown =
        append_authority(&router, REGION, "no-such-cluster", Some("tok-unknown")).await;
    assert_eq!(
        appended_to_unknown.status, 404,
        "appending to an unknown cluster is not found, got {appended_to_unknown:?}"
    );
    assert_eq!(
        appended_to_unknown.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {appended_to_unknown:?}"
    );

    // The other region's cluster kept its own CAs: appending there succeeds rather
    // than colliding with this region's successor.
    let other = append_authority(&router, OTHER_REGION, "shared-name", Some("tok-other")).await;
    assert_eq!(
        other.status, 200,
        "the same-named cluster in another region has its own room, got {other:?}"
    );
    assert_ne!(
        other.body["certificateAuthority"]["id"],
        serde_json::json!(appended_id),
        "two regions' certificate authorities must not share an id"
    );
}

// ---------------------------------------------------------------------------
// RULE: a cluster that is not ACTIVE cannot take a change to its certificate
// authorities; the refusal names the status it is in and nothing is written.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_that_is_not_active_refuses_the_append() {
    let router = emulator();
    let cases = [("one cluster", "settling-ca-a"), ("another", "settling-ca-b")];

    for (case, cluster) in cases {
        let created = create_cluster(&router, REGION, cluster).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateCluster, got {created:?}"
        );
        assert_eq!(
            created.body["cluster"]["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a brand new cluster is CREATING, got {:?}",
            created.body["cluster"]["status"]
        );

        // The transient state is observable, so its state error is too.
        let refused = append_authority(&router, REGION, cluster, Some("tok-creating")).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected 409 while the cluster is CREATING, got {refused:?}"
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {refused:?}"
        );
    }

    settle(CLUSTER_ACTIVATION_DELAY).await;

    for (case, cluster) in cases {
        // Once the clock has made the cluster ACTIVE the same call is accepted — and
        // the refused one wrote nothing, so this is the cluster's first successor.
        let appended = append_authority(&router, REGION, cluster, Some("tok-creating")).await;
        assert_eq!(
            appended.status, 200,
            "case {case:?}: an ACTIVE cluster takes the append, got {appended:?}"
        );
        assert_eq!(
            appended.body["certificateAuthority"]["distributionStatus"],
            serde_json::json!("IN_PROGRESS"),
            "case {case:?}: the accepted append starts a fresh distribution, got {:?}",
            appended.body["certificateAuthority"]["distributionStatus"]
        );
    }
}
