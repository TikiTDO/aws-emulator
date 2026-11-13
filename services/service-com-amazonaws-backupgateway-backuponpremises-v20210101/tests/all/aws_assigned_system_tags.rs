// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AWS-ASSIGNED system tags a resource this API creates carries.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour
//! and the only way a later request can see what an earlier one wrote is the shared store.
//!
//! The rule under test: a resource created through this API carries the provenance tags AWS
//! assigns it — `aws:createdBy`, whose value names the identity the creating request was
//! signed by, and `aws:createdDate`, the instant the resource was created at — and
//! `ListTagsForResource` reports those two entries AHEAD of the client's own tags. They are
//! in the reserved `aws:` key namespace, so a client can neither write one (`TagResource`
//! refuses the key) nor remove one (`UntagResource` leaves it), and they do not move: the
//! same values come back from every later read of one resource, whatever the client does to
//! its own tags.
//!
//! The table below spans that class rather than one recorded request: both resource types a
//! request of this model creates (a gateway through `CreateGateway`, a hypervisor through
//! `ImportHypervisorConfiguration`), each with no client tags, with client tags carried BY
//! the create, and with client tags written afterwards — including a client key sorting
//! before `aws:` in byte order, which is what makes the leading POSITION a rule of its own
//! rather than a consequence of key order.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    is_reserved_tag_key, BACKUP_VM, CREATED_BY_TAG_KEY, CREATED_DATE_TAG_KEY,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, issued_activation_key, Reply, REGION,
};

/// The access key id the harness signs every request with
/// (`crate::gateway_lifecycle`'s `authorization`): `aws:createdBy` names the credential the
/// creating request presented, so the value is a function of the request and not a
/// constant this suite could only match by copying.
const SIGNING_ACCESS_KEY_ID: &str = "AKIAIOSFODNN7EXAMPLE";

/// One row: which resource type is created, the tags the create itself carries and the
/// tags a later `TagResource` writes.
struct Row {
    case: &'static str,
    kind: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    tagged_with: &'static [(&'static str, &'static str)],
}

/// The rows. `"Alpha"` and `"ZZZ"` bracket `aws:` in byte order — an upper-case letter
/// sorts below `a`, a lower-case one above it — so a row with each pins that the system
/// pair leads the list because it is the system pair, not because `aws:` happens to sort
/// first.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a gateway with no client tags",
            kind: "gateway",
            created_with: &[],
            tagged_with: &[],
        },
        Row {
            case: "a gateway whose create carried two client tags",
            kind: "gateway",
            created_with: &[("sim2real-k1", "v1"), ("sim2real-k2", "v2")],
            tagged_with: &[],
        },
        Row {
            case: "a gateway tagged afterwards, with a key sorting below 'aws:'",
            kind: "gateway",
            created_with: &[],
            tagged_with: &[("Alpha", "first"), ("zulu", "last")],
        },
        Row {
            case: "a hypervisor with no client tags",
            kind: "hypervisor",
            created_with: &[],
            tagged_with: &[],
        },
        Row {
            case: "a hypervisor whose import carried a client tag, tagged again afterwards",
            kind: "hypervisor",
            created_with: &[("env", "test")],
            tagged_with: &[("Alpha", "first"), ("team", "core")],
        },
        Row {
            case: "a hypervisor tagged with a key sorting above 'aws:'",
            kind: "hypervisor",
            created_with: &[],
            tagged_with: &[("zzz", "last")],
        },
    ]
}

pub(crate) fn tag_body(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Create one resource of `kind`, carrying `tags`, and hand back the ARN the store minted.
///
/// Shared with the sibling `system_tag_created_date_form` suite, so both read the system
/// tags of resources minted the one way.
pub(crate) async fn create_resource(
    router: &MultiServiceRouter,
    kind: &str,
    nonce: usize,
    tags: &[(&str, &str)],
) -> String {
    match kind {
        "gateway" => {
            let reply = call(
                router,
                "CreateGateway",
                &json!({
                    "ActivationKey": issued_activation_key(router).await,
                    "GatewayDisplayName": format!("gw-system-tags-{nonce}"),
                    "GatewayType": BACKUP_VM,
                    "Tags": tag_body(tags),
                }),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "CreateGateway must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            reply.gateway_arn().to_string()
        }
        "hypervisor" => {
            let reply = call(
                router,
                "ImportHypervisorConfiguration",
                &json!({
                    "Name": format!("hv-system-tags-{nonce}"),
                    "Host": format!("192.0.2.{}", 10 + nonce),
                    "Tags": tag_body(tags),
                }),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "ImportHypervisorConfiguration must answer 200, got {} {:?}",
                reply.status, reply.body
            );
            reply
                .str_at("/HypervisorArn")
                .unwrap_or_else(|| panic!("no HypervisorArn in {:?}", reply.body))
                .to_string()
        }
        other => panic!("unknown resource kind {other:?}"),
    }
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(router, "ListTagsForResource", &json!({ "ResourceArn": arn })).await
}

/// The whole `Tags` list `ListTagsForResource` reports for `arn`, in the order it reported
/// it, asserting the call was served.
pub(crate) async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
) -> Vec<(String, String)> {
    let reply = list_tags(router, arn).await;
    assert_eq!(
        (reply.status, reply.str_at("/ResourceArn")),
        (200, Some(arn)),
        "case {case:?}: ListTagsForResource must answer 200 echoing {arn}, got {} {:?}",
        reply.status,
        reply.body
    );
    reply.reported_tags()
}

/// A tag read reports the two AWS-assigned entries, in that order, ahead of every client
/// tag — and their values are the creating identity and a creation instant, neither of them
/// taken from the request body.
#[tokio::test]
async fn a_tag_read_reports_the_system_tags_ahead_of_the_client_tags() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut creation_dates: BTreeSet<String> = BTreeSet::new();
    for (nonce, row) in rows().iter().enumerate() {
        let arn = create_resource(&router, row.kind, nonce, row.created_with).await;
        if !row.tagged_with.is_empty() {
            let written = call(
                &router,
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": tag_body(row.tagged_with) }),
            )
            .await;
            assert_eq!(
                written.status, 200,
                "case {:?}: TagResource must answer 200, got {} {:?}",
                row.case, written.status, written.body
            );
        }

        let reported = reported_tags(&router, row.case, &arn).await;

        // The two AWS-assigned entries lead the list, in the order AWS reports them.
        let keys: Vec<&str> = reported.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys.first().copied().zip(keys.get(1).copied()),
            Some((CREATED_BY_TAG_KEY, CREATED_DATE_TAG_KEY)),
            "case {:?}: the AWS-assigned tags must lead the list, got {reported:?}",
            row.case
        );

        // `aws:createdBy` names the identity the creating request was signed by: the
        // credential it presented, and the IAM user that credential speaks for.
        let created_by = &reported[0].1;
        assert!(
            created_by.contains(SIGNING_ACCESS_KEY_ID) && created_by.starts_with("IAMUser:"),
            "case {:?}: aws:createdBy must name the signing identity, got {created_by:?}",
            row.case
        );

        // `aws:createdDate` reports the record's creation instant, in the one form the
        // `system_tag_created_date_form` suite states the rule for — asserted here through
        // that suite's own check, so neither suite carries a second copy of the rule.
        let created_date = reported[1].1.clone();
        crate::system_tag_created_date_form::assert_epoch_millis_form(row.case, &created_date);
        creation_dates.insert(created_date);

        // Everything after the first two is the client's own, exactly the pairs the create
        // carried and the tag write added, with no reserved key among them.
        let client: Vec<(String, String)> = reported[2..].to_vec();
        let mut expected: Vec<(String, String)> = row
            .created_with
            .iter()
            .chain(row.tagged_with.iter())
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        expected.sort();
        assert_eq!(
            client, expected,
            "case {:?}: the client's own tags must follow the AWS-assigned ones, got \
             {reported:?}",
            row.case
        );
        assert!(
            client.iter().all(|(key, _)| !is_reserved_tag_key(key)),
            "case {:?}: no client tag may be in the reserved namespace, got {client:?}",
            row.case
        );
    }

    assert!(
        !creation_dates.is_empty(),
        "every row must have reported a creation date, got {creation_dates:?}"
    );
}

/// The AWS-assigned entries are the service's, not the client's: `TagResource` refuses a
/// key in the reserved namespace, `UntagResource` naming one removes nothing, and both
/// entries read back unchanged after either attempt.
#[tokio::test]
async fn a_client_can_neither_write_nor_remove_an_aws_assigned_tag() {
    let engine = build_engine();
    let router = build_router(&engine);

    for (nonce, kind) in ["gateway", "hypervisor"].into_iter().enumerate() {
        let arn = create_resource(&router, kind, 40 + nonce, &[("env", "test")]).await;
        let before = reported_tags(&router, kind, &arn).await;

        // Writing one is refused, with the modelled `ValidationException` the reserved
        // prefix rule is stated through.
        for (case, key) in [
            ("the createdBy key itself", CREATED_BY_TAG_KEY),
            ("the createdDate key itself", CREATED_DATE_TAG_KEY),
            ("another key in the reserved namespace", "aws:own"),
            ("the reserved prefix in mixed case", "AWS:createdBy"),
        ] {
            let written = call(
                &router,
                "TagResource",
                &json!({ "ResourceARN": arn, "Tags": [{ "Key": key, "Value": "mine" }] }),
            )
            .await;
            assert_eq!(
                (written.status, written.code.as_deref()),
                (400, Some("ValidationException")),
                "case {kind} / {case:?}: writing a reserved key must be refused, got {} {:?}",
                written.status,
                written.body
            );
        }

        // Removing one removes nothing: the call succeeds (a key the resource's own tag set
        // does not hold is nothing to remove) and both entries are still reported.
        let removed = call(
            &router,
            "UntagResource",
            &json!({
                "ResourceARN": arn,
                "TagKeys": [CREATED_BY_TAG_KEY, CREATED_DATE_TAG_KEY],
            }),
        )
        .await;
        assert_eq!(
            removed.status, 200,
            "case {kind}: UntagResource naming a reserved key must answer 200, got {} {:?}",
            removed.status, removed.body
        );

        let after = reported_tags(&router, kind, &arn).await;
        assert_eq!(
            after, before,
            "case {kind}: no client call may change the AWS-assigned tags, got {after:?}"
        );
    }
}

/// The AWS-assigned entries are a fact about the record, so they are the same on every
/// read of one resource and each resource carries its own — a read reports them for a
/// resource of any region this service serves, and a tag read changes nothing.
#[tokio::test]
async fn the_system_tags_are_stable_per_resource_and_its_own() {
    let engine = build_engine();
    let router = build_router(&engine);

    let first = create_resource(&router, "hypervisor", 70, &[]).await;
    let second = create_resource(&router, "hypervisor", 71, &[]).await;

    // Repeated reads agree: nothing about the pair is minted per call.
    let reads: Vec<Vec<(String, String)>> = vec![
        reported_tags(&router, "the first read", &first).await,
        reported_tags(&router, "the second read", &first).await,
        reported_tags(&router, "the third read", &first).await,
    ];
    assert!(
        reads.windows(2).all(|pair| pair[0] == pair[1]),
        "repeated reads must report one pair, got {reads:?}"
    );

    // Two resources created by the same identity report the same creator, and each reports
    // its own creation instant — which is a fact about the record, so neither read moves
    // it.
    let other = reported_tags(&router, "the other resource", &second).await;
    assert_eq!(
        (reads[0][0].0.as_str(), reads[0][0].1.as_str()),
        (other[0].0.as_str(), other[0].1.as_str()),
        "two resources one identity created must report one creator, got {other:?}"
    );
    assert_eq!(
        other,
        reported_tags(&router, "the other resource read again", &second).await,
        "a read must leave the AWS-assigned tags exactly as they were, got {other:?}"
    );

    // And the resource the harness addressed is the one of its own region, so the read the
    // pair came back from resolved the record this service holds.
    assert!(
        first.contains(REGION) && second.contains(REGION),
        "the records read must be the ones minted in the region the harness signs for, got \
         {first:?} and {second:?}"
    );
}
