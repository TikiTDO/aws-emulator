// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DisassociateResolverQueryLogConfig`.
//!
//! The rule these pin: the call "disassociates a VPC from a query logging
//! configuration", so it takes out of state EXACTLY the one association the
//! ordered (configuration, VPC) pair it names was made from, answers that
//! record with `DELETING` ("Resolver is deleting this query logging
//! association"), refuses a pair it holds no association for with the
//! `ResourceNotFoundException` it models, and leaves every other association —
//! and every configuration — as it was.
//!
//! [`super::resolver_query_log_config_associations`] owns the other half of the
//! slice (what an associate writes and what the readers answer); what is
//! asserted here is the SCOPE of one disassociate — which records it may touch
//! — over the shapes an association graph takes (a lone association, one of
//! several VPCs one configuration logs for, one of several configurations one
//! VPC is associated with, one cell of a configuration-by-VPC grid) and over
//! the forms a `ResourceId` takes, including both published bounds of its
//! length 1..64 (`smithy_query(query="shape",shape="ResourceId")`).
//!
//! Every case goes over the wire the service actually speaks (awsJson1_1,
//! `POST /` with `X-Amz-Target: Route53Resolver.<OperationName>`), through the
//! same router the emulator mounts, so what is asserted is the HTTP behaviour
//! and not a handler called directly. The transport helpers are the shared ones
//! in [`super::harness`] and the fixtures the ones in
//! [`super::resolver_query_log_config_associations`].

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_route53resolver_route53resolver::ops::types::RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION_STATUS_VALUES;

use super::harness::{
    call, emulator, error_type, expect_error, ok, parse_time, string_member, traced_message,
    without_status, Emulator,
};
use super::resolver_query_log_config_associations::{association, association_count, config_id};

/// The longest `ResourceId` there is: 64 characters, the maximum of its length
/// 1..64.
const LONGEST_VPC_ID: &str =
    "vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv";

/// The `ResolverQueryLogConfigAssociationStatus` values an association that is
/// still in force may report: the transient one a new association carries and
/// the settled one the clock-owed transition moves it to
/// (`smithy_query(query="shape",shape="ResolverQueryLogConfigAssociationStatus")`).
const IN_FORCE_STATUSES: &[&str] = &["CREATING", "ACTIVE"];

/// The sentence Resolver answers for a query logging configuration it holds
/// nothing under: fixed text behind the service error code for that fault,
/// naming no id, before the trace clause the refusal closes with.
const CONFIG_GONE_SENTENCE: &str =
    "[RSLVR-01601] The specified query logging configuration doesn't exist.";

/// The sentence Resolver answers for an association it holds nothing under,
/// whether the request addressed it by its own id or by the (configuration,
/// VPC) pair it was made from: fixed text behind its own service error code,
/// which is what tells this fault from [`CONFIG_GONE_SENTENCE`]'s.
const ASSOCIATION_GONE_SENTENCE: &str =
    "[RSLVR-01602] The specified query logging configuration association doesn't exist.";

/// One row of the scope table.
struct DisassociateCase {
    case: &'static str,
    /// How many fixture configurations the row creates.
    configs: usize,
    /// The VPC ids the row associates, addressed by index from `pairs`.
    vpcs: &'static [&'static str],
    /// The associations the row makes, as (configuration index, VPC index), in
    /// the order it makes them.
    pairs: &'static [(usize, usize)],
    /// Index into `pairs` of the association the disassociate takes out.
    target: usize,
}

/// The rows differ along what the rule generalises over: how many associations
/// the store holds, whether the target shares its configuration or its VPC with
/// another association, where in association order it sits, and the form and
/// length of the VPC id.
const DISASSOCIATE_CASES: &[DisassociateCase] = &[
    DisassociateCase {
        case: "the only association of the only configuration",
        configs: 1,
        vpcs: &["vpc-0123456789abcdef0"],
        pairs: &[(0, 0)],
        target: 0,
    },
    DisassociateCase {
        case: "the first of three VPCs one configuration logs for",
        configs: 1,
        vpcs: &["vpc-0a1b2c3d", "vpc-0123456789abcdef0", "vpc-00000000000000001"],
        pairs: &[(0, 0), (0, 1), (0, 2)],
        target: 0,
    },
    DisassociateCase {
        case: "the last of three VPCs one configuration logs for",
        configs: 1,
        vpcs: &["vpc-0a1b2c3d", "vpc-0123456789abcdef0", "vpc-00000000000000001"],
        pairs: &[(0, 0), (0, 1), (0, 2)],
        target: 2,
    },
    DisassociateCase {
        case: "the middle of three configurations one VPC is associated with",
        configs: 3,
        vpcs: &["vpc-0123456789abcdef0"],
        pairs: &[(0, 0), (1, 0), (2, 0)],
        target: 1,
    },
    DisassociateCase {
        case: "one cell of a two-configuration, two-VPC grid",
        configs: 2,
        vpcs: &["vpc-0a1b2c3d", "vpc-0123456789abcdef0"],
        pairs: &[(0, 0), (0, 1), (1, 0), (1, 1)],
        target: 2,
    },
    DisassociateCase {
        case: "a one-character VPC id, ResourceId's minimum length",
        configs: 1,
        vpcs: &["v"],
        pairs: &[(0, 0)],
        target: 0,
    },
    DisassociateCase {
        case: "a 64-character VPC id, ResourceId's maximum length",
        configs: 1,
        vpcs: &[LONGEST_VPC_ID],
        pairs: &[(0, 0)],
        target: 0,
    },
];

/// One row's fixture: the configurations it created, the VPC ids it works with,
/// and the associations it made, in association order.
struct Fixture {
    router: Emulator,
    configs: Vec<String>,
    made: Vec<Value>,
}

/// Build one row's configurations and associations on a fresh emulator.
async fn fixture(row_index: usize, row: &DisassociateCase) -> Fixture {
    let router = emulator();
    let mut configs = Vec::new();
    for index in 0..row.configs {
        configs.push(config_id(&router, &format!("disassociate-{row_index}-{index}")).await);
    }
    // Every VPC an associate names is one of the account's own, which the
    // associate resolves against the Amazon EC2 store.
    router.hold_vpcs(row.vpcs);

    let mut made = Vec::new();
    for (config, vpc) in row.pairs {
        let answered = ok(
            &router,
            "AssociateResolverQueryLogConfig",
            json!({
                "ResolverQueryLogConfigId": configs[*config].clone(),
                "ResourceId": row.vpcs[*vpc],
            }),
        )
        .await;
        made.push(association(&answered).clone());
    }

    Fixture {
        router,
        configs,
        made,
    }
}

/// A disassociate takes out exactly the association its pair was made from: the
/// answer is that record with the status the request leaves it in, the record is
/// gone from its reader, the configuration no longer counts it, and every other
/// association — and every other configuration — is untouched.
#[tokio::test]
async fn a_disassociate_takes_out_exactly_the_association_its_pair_was_made_from() {
    assert_eq!(
        LONGEST_VPC_ID.len(),
        64,
        "the longest row must name a 64-character ResourceId, got {}",
        LONGEST_VPC_ID.len()
    );

    for (index, row) in DISASSOCIATE_CASES.iter().enumerate() {
        let case = row.case;
        let Fixture {
            router,
            configs,
            made,
        } = fixture(index, row).await;

        let (target_config, target_vpc) = row.pairs[row.target];
        let config_id = configs[target_config].clone();
        let vpc_id = row.vpcs[target_vpc];
        let target = made[row.target].clone();
        let target_association_id = string_member(&target, "Id");

        // What each configuration counted before the call, so the drop can be
        // attributed to the one configuration the request named.
        let mut counts_before = Vec::new();
        for config in &configs {
            counts_before.push(association_count(&router, config).await);
        }

        let answered = ok(
            &router,
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": config_id.clone(), "ResourceId": vpc_id }),
        )
        .await;
        let answered = association(&answered).clone();

        assert_eq!(
            string_member(&answered, "Id"),
            target_association_id,
            "case {case:?}: the answer must be the association the pair was made from, got \
             {answered}"
        );
        assert_eq!(
            (
                string_member(&answered, "ResolverQueryLogConfigId"),
                string_member(&answered, "ResourceId"),
            ),
            (config_id.clone(), vpc_id.to_string()),
            "case {case:?}: the answer must carry the pair the request named, got {answered}"
        );
        let status = string_member(&answered, "Status");
        assert_eq!(
            status, "DELETING",
            "case {case:?}: the association on its way out reports DELETING, got {answered}"
        );
        assert!(
            RESOLVER_QUERY_LOG_CONFIG_ASSOCIATION_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: Status must be a modelled value, got {status:?}"
        );
        // "If the value of Status is a value other than FAILED, Error is null",
        // and ErrorMessage with it.
        for member in ["Error", "ErrorMessage"] {
            assert!(
                answered.get(member).unwrap_or(&Value::Null).is_null(),
                "case {case:?}: {member} is null while the association has not failed, got \
                 {answered}"
            );
        }
        parse_time(
            case,
            "CreationTime",
            &string_member(&answered, "CreationTime"),
        );
        assert_eq!(
            without_status(&answered),
            without_status(&target),
            "case {case:?}: only Status may differ from the association that was made, got \
             {answered}"
        );

        // Gone from the reader that addresses it by its own id, and from a
        // second disassociate of the pair it was made from.
        expect_error(
            &router,
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": target_association_id.clone() }),
            case,
            "ResourceNotFoundException",
        )
        .await;
        expect_error(
            &router,
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": config_id.clone(), "ResourceId": vpc_id }),
            case,
            "ResourceNotFoundException",
        )
        .await;

        // The configuration the request named counts one VPC fewer; every other
        // configuration counts exactly what it did.
        for (position, config) in configs.iter().enumerate() {
            let expected = counts_before[position] - i64::from(position == target_config);
            assert_eq!(
                association_count(&router, config).await,
                expected,
                "case {case:?}: configuration {config:?} must count {expected} VPCs after the \
                 disassociate"
            );
        }

        // Every other association is exactly as it was and still in force.
        for (position, made_association) in made.iter().enumerate() {
            if position == row.target {
                continue;
            }
            let id = string_member(made_association, "Id");
            let read = ok(
                &router,
                "GetResolverQueryLogConfigAssociation",
                json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
            )
            .await;
            let read = association(&read).clone();
            assert_eq!(
                without_status(&read),
                without_status(made_association),
                "case {case:?}: association {id:?} must be untouched by the disassociate, got \
                 {read}"
            );
            let status = string_member(&read, "Status");
            assert!(
                IN_FORCE_STATUSES.contains(&status.as_str()),
                "case {case:?}: association {id:?} must still be in force, got {status:?}"
            );
        }
    }
}

/// The pair is ORDERED and both halves must match: a configuration the store
/// holds nothing under, a VPC that configuration was never associated with, a
/// VPC associated with ANOTHER configuration, and the two members swapped all
/// address no association the store holds. Each is refused with
/// `ResourceNotFoundException`, nothing is taken out, and the sentence is the
/// service's own for the resource that is missing — the configuration
/// ([`CONFIG_GONE_SENTENCE`]) or the association itself
/// ([`ASSOCIATION_GONE_SENTENCE`]). Both sentences are fixed text, so the
/// bracketed service error code, and not an id echoed into the message, is what
/// tells the two faults apart.
#[tokio::test]
async fn a_pair_the_store_holds_no_association_for_is_refused() {
    let router = emulator();
    let first = config_id(&router, "mismatch-first").await;
    let second = config_id(&router, "mismatch-second").await;
    let first_vpc = "vpc-0123456789abcdef0";
    let second_vpc = "vpc-0a1b2c3d";
    let unassociated_vpc = "vpc-00000000000000009";
    router.hold_vpcs([first_vpc, second_vpc, unassociated_vpc]);

    let made = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": first.clone(), "ResourceId": first_vpc }),
    )
    .await;
    let made = association(&made).clone();
    let other = ok(
        &router,
        "AssociateResolverQueryLogConfig",
        json!({ "ResolverQueryLogConfigId": second.clone(), "ResourceId": second_vpc }),
    )
    .await;
    let other = association(&other).clone();

    // A configuration the store holds nothing under is answered for THAT, with
    // the one fixed sentence every query-logging operation reports a missing
    // configuration with.
    let mut config_messages: Vec<String> = Vec::new();
    for (case, unknown) in [
        ("an id nothing minted", "rqlc-0123456789abcdef".to_string()),
        ("an association id", "rqlca-0123456789abcdef".to_string()),
        ("one character", "x".to_string()),
        ("a near miss of a stored id", first[..first.len() - 1].to_string()),
    ] {
        let (status, parsed) = call(
            &router,
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": unknown.clone(), "ResourceId": first_vpc }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unknown configuration must answer HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ResourceNotFoundException"),
            "case {case:?}: the refusal must name ResourceNotFoundException, got {parsed}"
        );
        let message = string_member(&parsed, "Message");
        let (stated, _) = traced_message(&message).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must close with the trace clause of the request, got \
                 {message:?}"
            )
        });
        assert_eq!(
            stated, CONFIG_GONE_SENTENCE,
            "case {case:?}: the refusal must carry the service's own coded sentence, got {parsed}"
        );
        assert_eq!(
            string_member(&parsed, "ResourceType"),
            "ResolverQueryLogConfig",
            "case {case:?}: the refusal must name the resource that is missing, got {parsed}"
        );
        config_messages.push(stated.to_string());
    }
    assert_eq!(
        config_messages.iter().collect::<BTreeSet<&String>>().len(),
        1,
        "every configuration id the store holds nothing under must give the SAME fixed Message, \
         got {config_messages:?}"
    );

    // A stored configuration paired with a VPC it holds no association with is
    // answered for the ASSOCIATION, with that fault's own coded sentence.
    let mut pair_messages: Vec<String> = Vec::new();
    for (case, config, vpc) in [
        (
            "a VPC the configuration was never associated with",
            first.clone(),
            unassociated_vpc.to_string(),
        ),
        (
            "a VPC associated with another configuration",
            first.clone(),
            second_vpc.to_string(),
        ),
        (
            "a configuration the VPC is not associated with",
            second.clone(),
            first_vpc.to_string(),
        ),
        (
            "the two members swapped",
            first_vpc.to_string(),
            first.clone(),
        ),
    ] {
        let (status, parsed) = call(
            &router,
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": vpc.clone() }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a pair with no association must answer HTTP 400, got {status}: {parsed}"
        );
        assert!(
            error_type(&parsed).contains("ResourceNotFoundException"),
            "case {case:?}: the refusal must name ResourceNotFoundException, got {parsed}"
        );
        let message = string_member(&parsed, "Message");
        let (stated, _) = traced_message(&message).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must close with the trace clause of the request, got \
                 {message:?}"
            )
        });
        // The swapped row names a configuration the store holds nothing under,
        // so it is answered for the configuration; the others name a stored
        // configuration, so they are answered for the association itself.
        let expected = if config.starts_with("rqlc-") {
            ASSOCIATION_GONE_SENTENCE
        } else {
            CONFIG_GONE_SENTENCE
        };
        assert_eq!(
            stated, expected,
            "case {case:?}: the refusal must carry the coded sentence of the resource that is \
             missing, got {parsed}"
        );
        pair_messages.push(stated.to_string());
    }
    assert_eq!(
        pair_messages.iter().collect::<BTreeSet<&String>>().len(),
        2,
        "the rows must answer exactly the two fixed sentences, got {pair_messages:?}"
    );

    // Nothing was taken out: both associations still read back, both
    // configurations still count their own VPC, and the pairs in the documented
    // order still disassociate.
    for (case, expected) in [("the first pair", &made), ("the second pair", &other)] {
        let id = string_member(expected, "Id");
        let read = ok(
            &router,
            "GetResolverQueryLogConfigAssociation",
            json!({ "ResolverQueryLogConfigAssociationId": id.clone() }),
        )
        .await;
        assert_eq!(
            without_status(association(&read)),
            without_status(expected),
            "case {case:?}: a refused disassociate must leave the association in place, got {read}"
        );
    }
    for (case, config) in [("the first configuration", &first), ("the second", &second)] {
        assert_eq!(
            association_count(&router, config).await,
            1,
            "case {case:?}: a refused disassociate must count the same VPCs as before"
        );
    }
    for (case, config, vpc) in [
        ("the first pair", first.clone(), first_vpc),
        ("the second pair", second.clone(), second_vpc),
    ] {
        let answered = ok(
            &router,
            "DisassociateResolverQueryLogConfig",
            json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": vpc }),
        )
        .await;
        assert_eq!(
            string_member(association(&answered), "ResourceId"),
            vpc,
            "case {case:?}: the pair in the documented order must disassociate its association, \
             got {answered}"
        );
        assert_eq!(
            association_count(&router, &config).await,
            0,
            "case {case:?}: the configuration counts no VPC once its last one is disassociated"
        );
    }
}

/// A disassociate frees the pair: the very same VPC may be associated with that
/// configuration again, which mints a NEW association rather than reviving the
/// one that was taken out.
#[tokio::test]
async fn a_disassociated_vpc_can_be_associated_again() {
    let router = emulator();
    let config = config_id(&router, "reassociate").await;
    let vpc = "vpc-0123456789abcdef0";
    router.hold_vpcs([vpc]);
    let request = json!({ "ResolverQueryLogConfigId": config.clone(), "ResourceId": vpc });

    let mut seen: Vec<String> = Vec::new();
    for round in 0..3 {
        let case = format!("round {round}");
        let made = ok(&router, "AssociateResolverQueryLogConfig", request.clone()).await;
        let id = string_member(association(&made), "Id");
        assert!(
            !seen.contains(&id),
            "case {case:?}: a re-associate must mint a new association, got {id:?} among {seen:?}"
        );
        seen.push(id.clone());
        assert_eq!(
            association_count(&router, &config).await,
            1,
            "case {case:?}: the configuration counts the VPC that is associated again"
        );

        let answered = ok(
            &router,
            "DisassociateResolverQueryLogConfig",
            request.clone(),
        )
        .await;
        assert_eq!(
            string_member(association(&answered), "Id"),
            id,
            "case {case:?}: the disassociate must answer the association in force, got {answered}"
        );
        assert_eq!(
            association_count(&router, &config).await,
            0,
            "case {case:?}: the configuration counts no VPC once it is disassociated"
        );
    }
}
