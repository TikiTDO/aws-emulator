// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Resolver query-logging association slice:
//! `AssociateResolverQueryLogConfig` and `GetResolverQueryLogConfigAssociation`.
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`].

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION_STATUS_VALUES;
use service_com_amazonaws_route53resolver_route53resolver::state::PROVISIONING_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, emulator, error_type, expect_error, ok, parse_time, string_member, traced_message,
    without_status, Emulator,
};

/// The destination every configuration these cases create delivers to: one
/// Amazon S3 bucket of the account, which the create resolves its
/// `DestinationArn` against.
const DESTINATION: &str = "arn:aws:s3:::amzn-s3-demo-bucket";

/// The `ResolverQueryLogConfigAssociation` of an associate/get response.
pub fn association(body: &Value) -> &Value {
    body.get("ResolverQueryLogConfigAssociation").unwrap_or_else(|| {
        panic!("the response carries a ResolverQueryLogConfigAssociation, got {body}")
    })
}

/// Wait out the span the clock owes an association that is still `CREATING`, so
/// the next call sees the settled `ACTIVE` status.
async fn settle() {
    tokio::time::sleep(PROVISIONING_DELAY + Duration::from_millis(250)).await;
}

/// Create a query logging configuration to associate VPCs with, and answer its
/// `Id`.
///
/// The destination the create names is one the account holds: a create resolves
/// its `DestinationArn` against the store of the service that owns it.
pub async fn config_id(router: &Emulator, name: &str) -> String {
    router.hold_query_log_destinations([DESTINATION]);
    let created = ok(
        router,
        "CreateResolverQueryLogConfig",
        json!({
            "CreatorRequestId": format!("req-{name}"),
            "Name": name,
            "DestinationArn": DESTINATION,
        }),
    )
    .await;
    string_member(
        created
            .get("ResolverQueryLogConfig")
            .unwrap_or_else(|| panic!("the create answers a configuration, got {created}")),
        "Id",
    )
}

/// The `AssociationCount` `GetResolverQueryLogConfig` reports for `id`.
pub async fn association_count(router: &MultiServiceRouter, id: &str) -> i64 {
    let fetched = ok(
        router,
        "GetResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": id }),
    )
    .await;
    fetched
        .get("ResolverQueryLogConfig")
        .and_then(|config| config.get("AssociationCount"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the configuration reports an AssociationCount, got {fetched}"))
}

/// One row of the associate table: the VPCs to associate with one freshly
/// created configuration.
struct AssociateCase {
    case: &'static str,
    config_name: &'static str,
    vpcs: Vec<String>,
}

/// An association between a VPC and a query logging configuration is persisted:
/// every member of the response is computed from the request plus what the model
/// documents, `GetResolverQueryLogConfigAssociation` answers that same
/// association, and the configuration's `AssociationCount` counts the VPCs
/// associated with it.
///
/// The table spans what the rule generalises over: how many VPCs one
/// configuration is associated with (one, several — "to associate more than one
/// VPC with a configuration, submit one AssociateResolverQueryLogConfig request
/// for each VPC"), and the forms a `ResourceId` takes, including both published
/// bounds of its length 1..64
/// (`smithy_query(query="shape",shape="ResolverQueryLogConfigAssociation")`).
#[tokio::test]
async fn an_associated_vpc_is_read_back_from_state() {
    let cases = vec![
        AssociateCase {
            case: "one VPC",
            config_name: "one-vpc",
            vpcs: vec!["vpc-0123456789abcdef0".to_string()],
        },
        AssociateCase {
            case: "three VPCs, one request each",
            config_name: "three-vpcs",
            vpcs: vec![
                "vpc-1a2b3c4d".to_string(),
                "vpc-0000000000000000a".to_string(),
                "vpc-ffffffffffffffff0".to_string(),
            ],
        },
        AssociateCase {
            case: "a one-character resource id, ResourceId's minimum length",
            config_name: "shortest-id",
            vpcs: vec!["v".to_string()],
        },
        AssociateCase {
            case: "a 64-character resource id, ResourceId's maximum length",
            config_name: "longest-id",
            // 64 characters: the most ResourceId's length 1..64 allows.
            vpcs: vec!["v".repeat(64)],
        },
    ];

    let router = emulator();
    let mut seen_ids: Vec<String> = Vec::new();

    for AssociateCase {
        case,
        config_name,
        vpcs,
    } in cases
    {
        let config = config_id(&router, config_name).await;
        assert_eq!(
            association_count(&router, &config).await,
            0,
            "case {case:?}: a configuration starts with no VPC associated"
        );

        for (index, vpc) in vpcs.iter().enumerate() {
            // The VPC is one of the account's own, which the associate resolves
            // against the Amazon EC2 store.
            router.hold_vpcs([vpc]);
            let made = ok(
                &router,
                "AssociateResolverQueryLogConfig",
                json!({
                    "ResolverQueryLogConfigId": config.clone(),
                    "ResourceId": vpc.clone(),
                }),
            )
            .await;
            let made = association(&made).clone();
            let id = string_member(&made, "Id");

            assert!(
                id.starts_with("rqlca-"),
                "case {case:?}: an association id is minted as rqlca-<hex>, got {id:?}"
            );
            assert!(
                !id.is_empty() && id.len() <= 64,
                "case {case:?}: Id must satisfy ResourceId's length 1..64, got {id:?}"
            );
            assert!(
                !seen_ids.contains(&id),
                "case {case:?}: each associate mints a distinct Id, {id:?} is already among {seen_ids:?}"
            );
            seen_ids.push(id.clone());

            assert_eq!(
                string_member(&made, "ResolverQueryLogConfigId"),
                config,
                "case {case:?}: the association must name the configuration the request did, got {made}"
            );
            assert_eq!(
                string_member(&made, "ResourceId"),
                *vpc,
                "case {case:?}: the association must name the VPC the request did, got {made}"
            );
            let status = string_member(&made, "Status");
            assert_eq!(
                status, "CREATING",
                "case {case:?}: the associate answers while Resolver is still creating it, got {made}"
            );
            assert!(
                RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION_STATUS_VALUES.contains(&status.as_str()),
                "case {case:?}: Status must be a modelled value, got {status:?}"
            );
            // "If the value of Status is a value other than FAILED, Error is
            // null", and ErrorMessage with it.
            for member in ["Error", "ErrorMessage"] {
                assert!(
                    made.get(member).unwrap_or(&Value::Null).is_null(),
                    "case {case:?}: {member} is null while the association has not failed, got {made}"
                );
            }
            parse_time(case, "CreationTime", &string_member(&made, "CreationTime"));

            // Round trip: the reader answers the record the associate wrote.
            let fetched = ok(
                &router,
                "GetResolverQueryLogConfigAssociation",
                json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
            )
            .await;
            // `Status` is the one member the clock is allowed to move, so it is
            // compared apart.
            assert_eq!(
                without_status(association(&fetched)),
                without_status(&made),
                "case {case:?}: GetResolverQueryLogConfigAssociation must answer the stored association, got {fetched}"
            );

            // The configuration counts the VPCs associated with it, so the count
            // and the associations cannot disagree.
            assert_eq!(
                association_count(&router, &config).await,
                i64::try_from(index + 1).expect("the case count fits an i64"),
                "case {case:?}: AssociationCount must count the {} VPCs associated so far",
                index + 1
            );
        }
    }

    // Every association made above is still readable after the later associates,
    // so an associate never disturbs the records beside it.
    settle().await;
    for id in &seen_ids {
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": id }),
        )
        .await;
        assert_eq!(
            string_member(association(&fetched), "Id"),
            *id,
            "case \"all associations survive\": {id} must still be readable, got {fetched}"
        );
    }
}

/// One VPC may be associated with several configurations, and one configuration
/// with several VPCs: what identifies an association is the PAIR, so each pair
/// gets its own record and each configuration counts only its own.
#[tokio::test]
async fn one_vpc_associates_with_several_configurations() {
    let router = emulator();
    let first = config_id(&router, "first-config").await;
    let second = config_id(&router, "second-config").await;
    let vpc = "vpc-0123456789abcdef0";
    router.hold_vpcs([vpc]);

    let mut ids: Vec<String> = Vec::new();
    for (case, config) in [("the first configuration", &first), ("the second", &second)] {
        let made = ok(
            &router,
            "AssociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": vpc }),
        )
        .await;
        let id = string_member(association(&made), "Id");
        assert!(
            !ids.contains(&id),
            "case {case:?}: each pair gets its own association, got {id:?} among {ids:?}"
        );
        assert_eq!(
            string_member(association(&made), "ResolverQueryLogConfigId"),
            *config,
            "case {case:?}: the association must name its own configuration, got {made}"
        );
        ids.push(id);
    }

    for (case, config) in [("the first configuration", &first), ("the second", &second)] {
        assert_eq!(
            association_count(&router, config).await,
            1,
            "case {case:?}: a configuration counts only the VPCs associated with it"
        );
    }
}

/// An association answers `CREATING` while the creation the clock owes is still
/// outstanding, and `ACTIVE` once it has fallen due — without any call having to
/// move it.
#[tokio::test]
async fn an_association_settles_from_creating_to_active() {
    let router = emulator();
    let config = config_id(&router, "settling-association").await;
    router.hold_vpcs(["vpc-0123456789abcdef0"]);
    let made = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({
            "ResolverQueryLogConfigId": config,
            "ResourceId": "vpc-0123456789abcdef0",
        }),
    )
    .await;
    let id = string_member(association(&made), "Id");
    assert_eq!(
        string_member(association(&made), "Status"),
        "CREATING",
        "case \"straight after the associate\": Resolver is still creating it, got {made}"
    );

    // Still creating well before the transition falls due: the span is never
    // zero, so a caller reading at once observes the transient status.
    let early = ok(
        &router,
        "GetResolverQueryLogConfigAssociation",
        json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
    )
    .await;
    assert_eq!(
        string_member(association(&early), "Status"),
        "CREATING",
        "case \"before the transition is due\": the status has not moved yet, got {early}"
    );

    settle().await;
    let settled = ok(
        &router,
        "GetResolverQueryLogConfigAssociation",
        json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
    )
    .await;
    assert_eq!(
        string_member(association(&settled), "Status"),
        "ACTIVE",
        "case \"after the transition is due\": the association is active, got {settled}"
    );

    // A read does not change what a later caller sees: the settled status is
    // reported again, and no other member moved with it.
    let again = ok(
        &router,
        "GetResolverQueryLogConfigAssociation",
        json!({ "ResolverQueryLogConfigAssociationId": id }),
    )
    .await;
    assert_eq!(
        association(&again),
        association(&settled),
        "case \"a second read\": reporting an association must not change it, got {again}"
    );
}

/// A `ResolverQueryLogConfigId` the store holds no configuration under is
/// refused with the `ResourceNotFoundException` the operation models, and
/// nothing is associated for it.
///
/// The rows walk what the rule generalises over: an id of the right shape that
/// nothing minted, an id of another of this service's resources, the shortest
/// `ResourceId` there is, and a near miss of a stored id.
#[tokio::test]
async fn an_unknown_configuration_id_is_refused() {
    let router = emulator();
    let present = config_id(&router, "present-config").await;
    router.hold_vpcs(["vpc-0123456789abcdef0"]);

    for (case, id) in [
        ("an id nothing minted", "rqlc-0123456789abcdef".to_string()),
        ("a Resolver rule id", "rslvr-rr-0123456789abcdef".to_string()),
        ("an association id", "rqlca-0123456789abcdef".to_string()),
        ("one character", "x".to_string()),
        (
            "a near miss of a stored id",
            present[..present.len() - 1].to_string(),
        ),
    ] {
        expect_error(
            &router,
            "AssociateResolverQueryLogConfig",
            json!({
                "ResolverQueryLogConfigId": id,
                "ResourceId": "vpc-0123456789abcdef0",
            }),
            case,
            "ResourceNotFoundException",
        )
        .await;
    }

    // The refusals above are about the configuration id, not about refusing
    // every associate, and none of them wrote an association.
    let made = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({
            "ResolverQueryLogConfigId": present.clone(),
            "ResourceId": "vpc-0123456789abcdef0",
        }),
    )
    .await;
    assert_eq!(
        string_member(association(&made), "ResolverQueryLogConfigId"),
        present,
        "case \"the stored configuration\": the associate must be served, got {made}"
    );
    assert_eq!(
        association_count(&router, &present).await,
        1,
        "case \"the stored configuration\": only the served associate counts"
    );
}

/// Associating a VPC that is already associated with that configuration is
/// refused with the `ResourceExistsException` the operation models, and leaves
/// the association that is already there exactly as it was.
#[tokio::test]
async fn a_vpc_already_associated_is_refused() {
    let router = emulator();
    let config = config_id(&router, "duplicate-vpc").await;

    for (case, vpc) in [
        ("a typical VPC id", "vpc-0123456789abcdef0"),
        ("a one-character resource id", "v"),
    ] {
        router.hold_vpcs([vpc]);
        let request = json!({
            "ResolverQueryLogConfigId": config.clone(),
            "ResourceId": vpc,
        });
        let made = ok(&router, "AssociateResolverQueryLogConfig", request.clone()).await;
        let id = string_member(association(&made), "Id");
        let count = association_count(&router, &config).await;

        let (status, body) = call(&router, "AssociateResolverQueryLogConfig", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a VPC already associated must be refused, got {status}: {body}"
        );
        assert!(
            error_type(&body).contains("ResourceExistsException"),
            "case {case:?}: the refusal must name ResourceExistsException, got {body}"
        );
        assert_eq!(
            association_count(&router, &config).await,
            count,
            "case {case:?}: the refused associate must write nothing"
        );
        let fetched = ok(
            &router,
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
        )
        .await;
        assert_eq!(
            string_member(association(&fetched), "ResourceId"),
            vpc,
            "case {case:?}: the association already stored must be untouched, got {fetched}"
        );
    }
}

/// `GetResolverQueryLogConfigAssociation` reports a
/// `ResolverQueryLogConfigAssociationId` the store holds no association under
/// with the `ResourceNotFoundException` it models — never an empty success, and
/// never an association it invented.
#[tokio::test]
async fn an_unknown_association_id_is_reported_as_missing() {
    let router = emulator();
    let config = config_id(&router, "readable-association").await;
    router.hold_vpcs(["vpc-0123456789abcdef0"]);
    let made = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({
            "ResolverQueryLogConfigId": config,
            "ResourceId": "vpc-0123456789abcdef0",
        }),
    )
    .await;
    let present = string_member(association(&made), "Id");

    for (case, id) in [
        ("an id nothing minted", "rqlca-0123456789abcdef".to_string()),
        ("the configuration's own id", "rqlc-0123456789abcdef".to_string()),
        ("one character", "x".to_string()),
        (
            "a near miss of a stored id",
            present[..present.len() - 1].to_string(),
        ),
    ] {
        expect_error(
            &router,
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
            case,
            "ResourceNotFoundException",
        )
        .await;

        // Every Resolver fault answers HTTP 400, so the bracketed service error
        // code inside `Message` is what tells this one apart. The sentence is
        // the service's own and is fixed text: it names neither the association
        // the caller asked for nor the configuration it would belong to, so the
        // same sentence comes back whichever id was named, closed with the trace
        // clause of the request it refused.
        let (_, body) = call(
            &router,
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
        )
        .await;
        let message = string_member(&body, "Message");
        let (stated, _) = traced_message(&message).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must close with the trace clause of the request, got \
                 {message:?}"
            )
        });
        assert_eq!(
            stated,
            "[RSLVR-01602] The specified query logging configuration association doesn't exist.",
            "case {case:?}: the refusal must carry the service's own coded sentence, got {body}"
        );
    }

    // The refusals above are about the id, not about refusing every read.
    let fetched = ok(
        &router,
        "GetResolverQueryLogConfigAssociation",
        json!({ "ResolverQueryLogConfigAssociationId": present.clone() }),
    )
    .await;
    assert_eq!(
        string_member(association(&fetched), "Id"),
        present,
        "case \"the stored id\": the read must answer the stored association, got {fetched}"
    );
}
